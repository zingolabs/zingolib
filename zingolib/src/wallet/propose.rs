//! creating proposals from wallet data

use zcash_address::ZcashAddress;
use zcash_client_backend::{
    data_api::{
        MaxSpendMode,
        wallet::{
            ConfirmationsPolicy,
            input_selection::{GreedyInputSelector, LockedInputPolicy, SpendPolicy},
            propose_send_max_transfer,
        },
    },
    fees::{DustAction, DustOutputPolicy},
    zip321::TransactionRequest,
};
use zcash_protocol::{
    ShieldedPool,
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    memo::{Memo, MemoBytes},
    value::Zatoshis,
};

use super::{
    LightWallet,
    error::{ProposeSendError, ProposeShieldError, WalletError},
};
use crate::{
    ZENNIES_FOR_ZINGO_AMOUNT,
    config::ChainType,
    data::{
        proposal::{ProportionalFeeProposal, ZingoProposal},
        receivers::{Receiver, transaction_request_from_receivers},
    },
    get_zennies_for_zingo_address,
    utils::conversion::address_from_str,
};
use pepper_sync::{
    keys::transparent::TransparentScope,
    sync::{ScanPriority, ScanRange},
};

impl LightWallet {
    /// Creates a proposal from a transaction request.
    pub(crate) fn create_send_proposal(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let memo = self.change_memo_from_transaction_request(&request);
        let input_selector = GreedyInputSelector::new();
        let chain_height =
            self.sync_state
                .last_known_chain_height()
                .ok_or(ProposeSendError::Proposal(
                    zcash_client_backend::data_api::error::Error::ScanRequired,
                ))?;
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            Some(memo),
            if self
                .chain_type
                .activation_height(NetworkUpgrade::Nu6_3)
                .is_some_and(|ironwood_height| chain_height >= ironwood_height)
            {
                ShieldedPool::Ironwood
            } else {
                ShieldedPool::Orchard
            },
            DustOutputPolicy::new(DustAction::AllowDustChange, None),
        );
        let chain_type = self.chain_type;

        zcash_client_backend::data_api::wallet::propose_transfer::<
            LightWallet,
            ChainType,
            GreedyInputSelector<LightWallet>,
            zcash_client_backend::fees::zip317::SingleOutputChangeStrategy<
                zcash_primitives::transaction::fees::zip317::FeeRule,
                LightWallet,
            >,
            WalletError,
        >(
            self,
            &chain_type,
            account_id,
            &input_selector,
            &change_strategy,
            request,
            // TODO: replace wallet min_confirmations field with confirmation policy to unify for all proposals
            ConfirmationsPolicy::new_symmetrical(self.wallet_settings.min_confirmations, false),
            &SpendPolicy::default(),
            None,
            None,
        )
        .map_err(|error| self.reserved_or(error))
    }

    /// Creates a proposal that sends the whole shielded spendable balance,
    /// less the fee, to `address`. With `zennies_for_zingo` set, a
    /// [`ZENNIES_FOR_ZINGO_AMOUNT`] payment is added and the send-all
    /// amount is reduced to cover it.
    ///
    /// The zenny variant is sized by a send-max proposal over every
    /// spendable note. The two-payment request is proposed once. If the
    /// extra output raised the fee, it is proposed a second time with the
    /// recipient amount reduced by that increase. The corrected request
    /// sums to the sized input total. Selection takes every note, the
    /// output count is unchanged, and the fee is the same. The second
    /// result is final.
    pub(crate) fn create_send_all_proposal(
        &mut self,
        address: ZcashAddress,
        zennies_for_zingo: bool,
        memo: Option<MemoBytes>,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        if !zennies_for_zingo {
            return self.propose_send_max(address, memo, account_id);
        }

        let sizing = self.propose_send_max(address.clone(), None, account_id)?;
        let overflow = || {
            ProposeSendError::Proposal(zcash_client_backend::data_api::error::Error::BalanceError(
                zcash_protocol::value::BalanceError::Overflow,
            ))
        };
        let sizing_fee = sizing
            .steps()
            .iter()
            .map(|step| step.balance().fee_required())
            .sum::<Option<Zatoshis>>()
            .ok_or_else(overflow)?;
        let max_to_recipient = recipient_amount(&sizing);
        let input_total = (max_to_recipient + sizing_fee).ok_or_else(overflow)?;
        let zenny_amount = Zatoshis::from_u64(ZENNIES_FOR_ZINGO_AMOUNT).expect("hard-coded");
        let Some(recipient_amount) =
            (max_to_recipient - zenny_amount).filter(|amount| *amount > Zatoshis::ZERO)
        else {
            let required = (zenny_amount + sizing_fee)
                .and_then(|value| value + Zatoshis::const_from_u64(1))
                .unwrap_or(Zatoshis::const_from_u64(zcash_protocol::value::MAX_MONEY));
            return Err(ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available: input_total,
                    required,
                },
            ));
        };

        let zenny_address = address_from_str(get_zennies_for_zingo_address(self.chain_type))
            .expect("hard-coded address");
        let request = |amount: Zatoshis| {
            transaction_request_from_receivers(vec![
                Receiver::new(address.clone(), amount, memo.clone()),
                Receiver::new(zenny_address.clone(), zenny_amount, None),
            ])
        };

        let first_attempt = self.create_send_proposal(request(recipient_amount)?, account_id);
        let required = match &first_attempt {
            Err(ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    required, ..
                },
            )) => *required,
            _ => return first_attempt,
        };
        let Some(corrected_amount) = (required - input_total)
            .and_then(|fee_increase| recipient_amount - fee_increase)
            .filter(|amount| *amount > Zatoshis::ZERO)
        else {
            return first_attempt;
        };
        self.create_send_proposal(request(corrected_amount)?, account_id)
    }

    fn propose_send_max(
        &mut self,
        address: ZcashAddress,
        memo: Option<MemoBytes>,
        account_id: zip32::AccountId,
    ) -> Result<ProportionalFeeProposal, ProposeSendError> {
        let chain_type = self.chain_type;
        let confirmations_policy =
            ConfirmationsPolicy::new_symmetrical(self.wallet_settings.min_confirmations, false);

        propose_send_max_transfer::<
            LightWallet,
            ChainType,
            zcash_primitives::transaction::fees::zip317::FeeRule,
            WalletError,
        >(
            self,
            &chain_type,
            account_id,
            &[
                ShieldedPool::Ironwood,
                ShieldedPool::Orchard,
                ShieldedPool::Sapling,
            ],
            &zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            address,
            memo,
            MaxSpendMode::MaxSpendable,
            confirmations_policy,
            &LockedInputPolicy::Exclude,
            None,
        )
        .map_err(|error| self.reserved_or(error))
    }

    fn reserved_or(
        &self,
        error: zcash_client_backend::data_api::error::Error<
            WalletError,
            WalletError,
            zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelectorError,
            zcash_primitives::transaction::fees::zip317::FeeError,
            zcash_primitives::transaction::fees::zip317::FeeError,
            crate::wallet::output::OutputRef,
        >,
    ) -> ProposeSendError {
        match error {
            zcash_client_backend::data_api::error::Error::InsufficientFunds { .. }
                if self.reserved_orchard_value() > 0 =>
            {
                ProposeSendError::ReservedForMigration {
                    reserved: self.reserved_orchard_value(),
                }
            }
            error => ProposeSendError::Proposal(error),
        }
    }

    /// The shield operation consumes a proposal that transfers value
    /// into the Orchard pool.
    ///
    /// The proposal is generated with this method, which operates on
    /// the balance transparent pool, without other input.
    /// In other words, shield does not take a user-specified amount
    /// to shield, rather it consumes all transparent value in the wallet that
    /// can be consumed without costing more in zip317 fees than is being transferred.
    pub(crate) fn create_shield_proposal(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<crate::data::proposal::ProportionalFeeShieldProposal, ProposeShieldError> {
        let input_selector = GreedyInputSelector::new();
        let change_strategy = zcash_client_backend::fees::zip317::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::zip317::FeeRule::standard(),
            None,
            ShieldedPool::Orchard,
            DustOutputPolicy::new(DustAction::AllowDustChange, None),
        );
        let chain_type = self.chain_type;

        // TODO: store t addrs as concrete types instead of encoded
        let transparent_addresses = self
            .transparent_addresses
            .values()
            .map(|address| {
                Ok(zcash_address::ZcashAddress::try_from_encoded(address)?
                    .convert_if_network::<zcash_transparent::address::TransparentAddress>(
                        self.chain_type.network_type(),
                    )
                    .expect("incorrect network should be checked on wallet load"))
            })
            .collect::<Result<Vec<_>, zcash_address::ParseError>>()?;

        let proposed_shield = zcash_client_backend::data_api::wallet::propose_shielding::<
            LightWallet,
            ChainType,
            GreedyInputSelector<LightWallet>,
            zcash_client_backend::fees::zip317::SingleOutputChangeStrategy<
                zcash_primitives::transaction::fees::zip317::FeeRule,
                LightWallet,
            >,
            WalletError,
        >(
            self,
            &chain_type,
            &input_selector,
            &change_strategy,
            Zatoshis::const_from_u64(10_000),
            &transparent_addresses,
            account_id,
            // TODO: replace wallet min_confirmations field with confirmation policy to unify for all proposals
            ConfirmationsPolicy::new_symmetrical(self.wallet_settings.min_confirmations, false),
            zcash_client_backend::data_api::CoinbaseFilter::AllTransparentOutputs,
            None,
        )
        .map_err(ProposeShieldError::Component)?;

        for step in proposed_shield.steps().iter() {
            if step
                .balance()
                .proposed_change()
                .iter()
                .fold(0, |total_out, output| total_out + output.value().into_u64())
                == 0
            {
                return Err(ProposeShieldError::InsufficientFunds);
            }
        }

        Ok(proposed_shield)
    }

    /// Stores a proposal in the `send_proposal` field.
    /// This field must be populated in order to then construct and transmit transactions.
    pub(crate) fn store_proposal(&mut self, proposal: ZingoProposal) {
        self.send_proposal = Some(proposal);
    }

    /// Takes the proposal from the `send_proposal` field, leaving the field empty.
    pub(crate) fn take_proposal(&mut self) -> Option<ZingoProposal> {
        self.send_proposal.take()
    }

    fn change_memo_from_transaction_request(&self, request: &TransactionRequest) -> MemoBytes {
        let mut recipient_uas = Vec::new();
        let mut refund_address_indexes = Vec::new();
        let mut refund_address_count = self
            .transparent_addresses
            .keys()
            .filter(|&address_id| address_id.scope() == TransparentScope::Refund)
            .count() as u32;
        for payment in request.payments().values() {
            if let Ok(address) = payment
                .recipient_address()
                .clone()
                .convert_if_network::<zcash_keys::address::Address>(self.chain_type.network_type())
            {
                match address {
                    zcash_keys::address::Address::Unified(unified_address) => {
                        recipient_uas.push(unified_address);
                    }
                    zcash_keys::address::Address::Tex(_) => {
                        refund_address_indexes.push(refund_address_count);
                        refund_address_count += 1;
                    }
                    _ => (),
                }
            }
        }
        let uas_bytes = match zingo_memo::create_wallet_internal_memo_version_1(
            &self.chain_type,
            recipient_uas.as_slice(),
            refund_address_indexes.as_slice(),
        ) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!(
                    "Could not write uas to memo field: {e}\n\
        Your wallet will display an incorrect sent-to address. This is a visual error only.\n\
        The correct address was sent to."
                );
                [0; 511]
            }
        };
        MemoBytes::from(Memo::Arbitrary(Box::new(uas_bytes)))
    }

    /// Returns the block height at which all blocks equal to and above this height are scanned (scan ranges whose
    /// priority satisfies [`ScanPriority::is_scanned`]).
    /// Returns `None` if `self.scan_ranges` is empty.
    ///
    /// Useful for determining which height all the nullifiers have been mapped from for guaranteeing if a note is
    /// unspent.
    ///
    /// The horizon *withholds* a note when the note's confirmation height lies below it. A spending
    /// transaction can be mined only at or above the block that mined the note, so a note at or
    /// above the horizon has had its entire spend window scanned, and the absence of a discovered
    /// spend proves the note unspent. For a note below the horizon, the unscanned gap may conceal
    /// a spend, so the strict form of [`Self::spendable_notes`] omits the note rather than vouch
    /// for it. Withholding asserts nothing about the note; it records only that the wallet does
    /// not yet know.
    ///
    /// `all_spends_known` may be set if all the spend locations are already known before scanning starts. For example,
    /// the location of all transparent spends are known due to the pre-scan gRPC calls. In this case, the height returned
    /// is the lowest height where there are no higher scan ranges with `FoundNote` or higher scan priority.
    pub(crate) fn spend_horizon(&self, all_spends_known: bool) -> Option<BlockHeight> {
        let mut scan_ranges_top_to_bottom = self.sync_state.scan_ranges().iter().rev();
        let awaits_spend_detection = |scan_range: &&ScanRange| {
            if all_spends_known {
                scan_range.priority() >= ScanPriority::FoundNote
                    || scan_range.priority() == ScanPriority::Scanning
            } else {
                !scan_range.priority().is_scanned()
            }
        };
        let highest_range_awaiting_detection =
            scan_ranges_top_to_bottom.find(awaits_spend_detection);
        highest_range_awaiting_detection
            .map(|awaiting_range| awaiting_range.block_range().end)
            .or_else(|| self.sync_state.wallet_birthday())
    }

    /// Returns `true` if all nullifiers above `note_height` have been checked for this note's spend status.
    ///
    /// Requires that `note_height >= spend_horizon` (all ranges above the note are scanned) and that every
    /// `refetch_nullifier_range` recorded on the note is fully contained within a `Scanned` scan range
    /// (nullifiers that were discarded due to memory constraints have since been re-fetched).
    pub(crate) fn note_spends_confirmed(
        &self,
        note_height: BlockHeight,
        spend_horizon: BlockHeight,
        refetch_nullifier_ranges: &[std::ops::Range<BlockHeight>],
    ) -> bool {
        note_height >= spend_horizon
            && refetch_nullifier_ranges.iter().all(|refetch_range| {
                self.sync_state.scan_ranges().iter().any(|scan_range| {
                    scan_range.priority() == ScanPriority::Scanned
                        && scan_range.block_range().contains(&refetch_range.start)
                        && scan_range.block_range().contains(&(refetch_range.end - 1))
                })
            })
    }
}

/// The amount of the first payment in a proposal's last step.
pub(crate) fn recipient_amount(proposal: &ProportionalFeeProposal) -> Zatoshis {
    proposal
        .steps()
        .last()
        .transaction_request()
        .payments()
        .get(&0)
        .and_then(|payment| payment.amount())
        .unwrap_or(Zatoshis::ZERO)
}

#[cfg(test)]
mod test {
    use zcash_protocol::{PoolType, ShieldedPool};

    use crate::{
        testutils::lightclient::from_inputs::transaction_request_from_send_inputs,
        testutils::synthetic_wallet::SyntheticWalletBuilder,
        wallet::keys::unified::ReceiverSelection,
    };

    /// Paying a unified address must target its best receiver: Orchard
    /// whenever the UA carries an orchard receiver, Sapling only when that
    /// is the best on offer. This is the guarantee the LocalNet test
    /// `diversified_addresses_receive_funds_in_best_pool` enforced with a
    /// full zebrad+zainod network. The proposal's payment-pool map states
    /// it directly from synthetic wallet data alone.
    #[test]
    fn proposal_targets_best_pool_per_unified_address() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .ironwood_note(100_000)
                .build();
        let chain = wallet.chain_type;

        let (_, orchard_only) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let (_, all_shielded) = wallet
            .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
            .unwrap();
        let (_, sapling_only) = wallet
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();
        let orchard_only = orchard_only.encode(&chain);
        let all_shielded = all_shielded.encode(&chain);
        let sapling_only = sapling_only.encode(&chain);

        let request = transaction_request_from_send_inputs(vec![
            (orchard_only.as_str(), 10_000, None),
            (all_shielded.as_str(), 10_000, None),
            (sapling_only.as_str(), 10_000, None),
        ])
        .expect("valid send inputs form a request");

        let proposal = wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .expect("synthetic wallet data supports proposing");

        let step = proposal.steps().first();
        let pools = step.payment_pools();
        assert_eq!(
            pools[&0],
            PoolType::Shielded(ShieldedPool::Ironwood),
            "orchard-only UA must be paid in ironwood"
        );
        assert_eq!(
            pools[&1],
            PoolType::Shielded(ShieldedPool::Ironwood),
            "all-shielded UA must be paid in its best pool, ironwood"
        );
        assert_eq!(
            pools[&2],
            PoolType::Shielded(ShieldedPool::Sapling),
            "sapling-only UA must be paid in sapling"
        );
    }

    /// Migrated from libtonode `propose_orchard_dust_to_sapling`: a wallet
    /// holding an ordinary orchard note and a dust note can propose a
    /// cross-pool send to a sapling address.
    /// FIXME: does not assert dust was included in the proposal (carried
    /// over from the original).
    #[test]
    fn propose_orchard_dust_to_sapling() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(100_000)
                .orchard_note(4_000)
                .build();

        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let (_, sapling_destination) = external_wallet
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();
        let sapling_destination = sapling_destination.encode(&external_wallet.chain_type());

        let request = transaction_request_from_send_inputs(vec![(
            sapling_destination.as_str(),
            10_000,
            None,
        )])
        .expect("valid send inputs form a request");

        wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .expect("orchard funds propose cleanly to a sapling destination");
    }

    /// Proposing a spend of existing funds works from wallet data alone, with
    /// no network. Formerly `#[ignore]`d ("for some reason this does not
    /// work without network"): it loaded an example wallet fixture, and
    /// fixtures deserialize without the confirmed-transaction state
    /// proposing requires. The synthetic builder fabricates that state.
    #[test]
    fn propose_100_000_to_self() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(200_000)
                .build();

        let pool = PoolType::Shielded(ShieldedPool::Orchard);
        let self_address = wallet.get_address(pool);

        let receivers = vec![(self_address.as_str(), 100_000, None)];
        let request = transaction_request_from_send_inputs(receivers)
            .expect("actually all of this logic oughta be internal to propose");

        wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .expect("can propose from existing data");
    }
}

#[cfg(test)]
mod migration_reservation {
    use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputInterface as _};
    use zcash_address::ZcashAddress;

    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::utils::conversion::address_from_str;
    use crate::wallet::LightWallet;
    use crate::wallet::error::ProposeSendError;
    use crate::wallet::keys::unified::ReceiverSelection;
    use crate::wallet::migration::{
        BoundNote, MigrationMode, MigrationParams, MigrationPhase, MigrationState, PlanCommitment,
        SigningStrategy, TransferId, TransferRecord,
    };

    const FUNDING_NOTE: u64 = 1_020_000;
    const RESIDUAL_NOTE: u64 = 50_000;
    const ACCOUNT: zip32::AccountId = zip32::AccountId::ZERO;

    fn external_address() -> ZcashAddress {
        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let (_, unified_address) = external_wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), ACCOUNT)
            .unwrap();
        address_from_str(&unified_address.encode(&external_wallet.chain_type()))
            .expect("a wallet-generated address parses")
    }

    fn migration_state(
        wallet: &LightWallet,
        phase: MigrationPhase,
        transfers: Vec<TransferRecord>,
    ) -> MigrationState {
        let params = MigrationParams::provisional(wallet.chain_type());
        MigrationState {
            commitment: PlanCommitment {
                params_hash: params.params_hash(),
                plan_hash: [0; 32],
                committed_at: 0,
            },
            params,
            strategy: SigningStrategy::LazyAtBoundary,
            mode: MigrationMode::Scheduled,
            account: ACCOUNT,
            phase,
            transfers,
        }
    }

    fn bound_note_of(wallet: &LightWallet, value: u64) -> BoundNote {
        wallet
            .wallet_transactions
            .values()
            .flat_map(OrchardNote::transaction_outputs)
            .find(|note| note.value() == value)
            .map(|note| BoundNote {
                output_id: note.output_id(),
                nullifier: note
                    .nullifier()
                    .expect("scanned notes carry nullifiers")
                    .to_bytes(),
                commitment: [0; 32],
            })
            .expect("the wallet holds the fabricated note")
    }

    fn assert_refused_as_reserved(
        refused: Result<crate::data::proposal::ProportionalFeeProposal, ProposeSendError>,
        expected: u64,
    ) {
        match refused {
            Err(ProposeSendError::ReservedForMigration { reserved }) => {
                assert_eq!(reserved, expected, "the refusal carries the reserved value");
            }
            Err(other) => panic!("expected ReservedForMigration, got {other:?}"),
            Ok(proposal) => panic!("expected ReservedForMigration, got a proposal: {proposal:?}"),
        }
    }

    #[test]
    fn send_all_over_only_reserved_notes_is_refused_as_reserved() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(FUNDING_NOTE)
                .orchard_note(RESIDUAL_NOTE)
                .build();
        wallet.migration = Some(migration_state(&wallet, MigrationPhase::Committed, vec![]));
        assert_eq!(
            wallet.reserved_orchard_value(),
            FUNDING_NOTE + RESIDUAL_NOTE
        );

        let refused = wallet.create_send_all_proposal(external_address(), false, None, ACCOUNT);

        assert_refused_as_reserved(refused, FUNDING_NOTE + RESIDUAL_NOTE);
    }

    #[test]
    fn send_all_with_zennies_over_only_reserved_notes_is_refused_as_reserved() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(FUNDING_NOTE)
                .build();
        wallet.migration = Some(migration_state(&wallet, MigrationPhase::Committed, vec![]));

        let refused = wallet.create_send_all_proposal(external_address(), true, None, ACCOUNT);

        assert_refused_as_reserved(refused, FUNDING_NOTE);
    }

    #[test]
    fn send_all_is_refused_as_reserved_when_the_only_note_is_a_bound_funding_note() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(FUNDING_NOTE)
                .build();
        let transfer = TransferRecord::new(
            TransferId(0),
            FUNDING_NOTE,
            bound_note_of(&wallet, FUNDING_NOTE),
        );
        wallet.migration = Some(migration_state(
            &wallet,
            MigrationPhase::Scheduled,
            vec![transfer],
        ));
        assert_eq!(wallet.reserved_orchard_value(), FUNDING_NOTE);

        let refused = wallet.create_send_all_proposal(external_address(), false, None, ACCOUNT);

        assert_refused_as_reserved(refused, FUNDING_NOTE);
    }

    #[test]
    fn send_all_over_free_notes_proposes_while_a_funding_note_is_reserved() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(FUNDING_NOTE)
                .orchard_note(RESIDUAL_NOTE)
                .build();
        let transfer = TransferRecord::new(
            TransferId(0),
            FUNDING_NOTE,
            bound_note_of(&wallet, FUNDING_NOTE),
        );
        wallet.migration = Some(migration_state(
            &wallet,
            MigrationPhase::Scheduled,
            vec![transfer],
        ));

        let proposal = wallet
            .create_send_all_proposal(external_address(), false, None, ACCOUNT)
            .expect("the free residual note pays the send-all");

        let step = proposal.steps().first();
        let selected: Vec<u64> = step
            .shielded_inputs()
            .expect("a shielded send-all selects shielded inputs")
            .notes()
            .iter()
            .map(|note| u64::from(note.note().value()))
            .collect();
        assert_eq!(
            selected,
            [RESIDUAL_NOTE],
            "the reserved funding note is never selected"
        );
    }

    #[test]
    fn send_all_with_no_reserved_value_reports_the_ordinary_insufficient_funds() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        assert_eq!(wallet.reserved_orchard_value(), 0);

        let refused = wallet.create_send_all_proposal(external_address(), false, None, ACCOUNT);

        assert!(
            matches!(refused, Err(ProposeSendError::Proposal(_))),
            "with nothing reserved the refusal is the plain proposal error: {refused:?}"
        );
    }
}
