//! This mod contains pieces of the impl `LightWallet` that are invoked during a send.

use std::ops::Range;

use nonempty::NonEmpty;

use zcash_client_backend::data_api::WalletRead as _;
use zcash_client_backend::data_api::wallet::SpendingKeys;
use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::util::SystemClock;

use pepper_sync::sync::{ScanPriority, ScanRange};
use pepper_sync::wallet::NoteInterface;
use zcash_primitives::transaction::fees::zip317;
use zcash_protocol::consensus::{BlockHeight, Parameters};
use zcash_protocol::{ShieldedPool, TxId};

use super::LightWallet;
use super::error::{CalculateTransactionError, KeyError};
use crate::wallet::utils;

impl LightWallet {
    /// Creates and stores transaction from the given `proposal`, returning the txids for each calculated transaction.
    pub(crate) async fn calculate_transactions<NoteRef>(
        &mut self,
        proposal: Proposal<zip317::FeeRule, NoteRef>,
        sending_account: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, CalculateTransactionError<NoteRef>> {
        let calculated_txids = match proposal.steps().len() {
            1 => {
                self.create_proposed_transactions(proposal, sending_account)
                    .await?
            }
            2 if proposal.steps()[1]
                .transaction_request()
                .payments()
                .values()
                .any(|payment| {
                    matches!(
                        payment
                            .recipient_address()
                            .clone()
                            .convert_if_network::<zcash_keys::address::Address>(
                                self.chain_type.network_type()
                            ),
                        Ok(zcash_keys::address::Address::Tex(_))
                    )
                }) =>
            {
                self.create_proposed_transactions(proposal, sending_account)
                    .await?
            }
            _ => return Err(CalculateTransactionError::NonTexMultiStep),
        };
        self.save_required = true;

        Ok(calculated_txids)
    }

    async fn create_proposed_transactions<NoteRef>(
        &mut self,
        proposal: Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
        sending_account: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, CalculateTransactionError<NoteRef>> {
        let chain_type = self.chain_type;
        let usk: zcash_keys::keys::UnifiedSpendingKey = self
            .unified_key_store
            .get(&sending_account)
            .ok_or(KeyError::NoAccountKeys)?
            .try_into()?;

        let sapling_prover = utils::LazySaplingProver::default();

        let expiry_height = self.proposal_expiry_height(&proposal);

        zcash_client_backend::data_api::wallet::create_proposed_transactions(
            self,
            &chain_type,
            &SystemClock,
            &mut crate::utils::system_rng(),
            &sapling_prover,
            &sapling_prover,
            &SpendingKeys::new(usk),
            zcash_client_backend::wallet::OvkPolicy::Sender,
            &proposal,
            expiry_height,
        )
        .map_err(CalculateTransactionError::Calculation)
    }

    /// The expiry height the wallet asks the backend to give a proposal's
    /// transactions, or `None` to leave the expiry to the backend.
    ///
    /// A step the backend builds as a canonical ZIP 318 crossing takes the
    /// ZIP's rolling expiry, which every crossing in a modulus period shares,
    /// and the backend refuses any other. A proposal holding one therefore
    /// leaves the expiry to the backend. Every other proposal takes the
    /// wallet's delta for its target height.
    pub(crate) fn proposal_expiry_height<NoteRef>(
        &self,
        proposal: &Proposal<zip317::FeeRule, NoteRef>,
    ) -> Option<BlockHeight> {
        let target_height = proposal.min_target_height();
        let holds_canonical_crossing = zcash_client_backend::fees::canonical_crossing_fee(
            &self.chain_type,
            target_height.into(),
        )
        .is_ok_and(|canonical_fee| {
            proposal.steps().iter().any(|step| {
                step.is_canonical_crossing(
                    &self.pool_migration_params(&self.chain_type),
                    canonical_fee,
                )
            })
        });
        (!holds_canonical_crossing).then(|| {
            crate::wallet::expiry::tx_expiry_height(&self.chain_type, target_height.into())
        })
    }

    /// Whether the witness of a note at `note_height` can be built as of `anchor_height`: the note's shard holds
    /// the leaves the witness path runs through, and the anchor's shard holds the leaves up to the anchor that the
    /// right-hand siblings of that path are computed from.
    pub(crate) fn can_build_witness<N>(
        &self,
        note_height: BlockHeight,
        anchor_height: BlockHeight,
    ) -> bool
    where
        N: NoteInterface,
    {
        self.shards_are_scanned(N::SHIELDED_PROTOCOL, Some(note_height), anchor_height)
    }

    /// Whether this wallet can materialize `protocol`'s note commitment tree root, and witnesses to it, as of `height`.
    pub(crate) fn anchor_is_computable(&self, protocol: ShieldedPool, height: BlockHeight) -> bool {
        self.shards_are_scanned(protocol, None, height)
            && self.checkpoint_is_retained(protocol, height)
    }

    /// Whether `protocol`'s shard tree retains a checkpoint at `height`.
    fn checkpoint_is_retained(&self, protocol: ShieldedPool, height: BlockHeight) -> bool {
        use shardtree::store::ShardStore;

        match protocol {
            ShieldedPool::Sapling => self.shard_trees.sapling.store().get_checkpoint(&height),
            ShieldedPool::Orchard => self.shard_trees.orchard.store().get_checkpoint(&height),
            ShieldedPool::Ironwood => self.shard_trees.ironwood.store().get_checkpoint(&height),
        }
        .expect("memory shard store is infallible")
        .is_some()
    }

    /// Whether the `protocol` shard holding `anchor_height`, and the one holding `note_height` when given, are
    /// scanned.
    fn shards_are_scanned(
        &self,
        protocol: ShieldedPool,
        note_height: Option<BlockHeight>,
        anchor_height: BlockHeight,
    ) -> bool {
        let Some(birthday) = self.sync_state.wallet_birthday() else {
            return false;
        };
        let scan_ranges = self.sync_state.scan_ranges();
        // The scan floor: the wallet's Birthday clamped to the Pool
        // Activation (ADR 0014, never a local mapping), the earliest
        // height that must be scanned before a note in this pool can be
        // witnessed. Each pool's commitment tree exists only from its
        // activation height. Sapling and Orchard implicitly rely on
        // `birthday >= activation` (true for all current wallets);
        // Ironwood makes the clamp explicit because wallets born before
        // NU6.3 can hold Ironwood notes immediately after activation.
        let scan_floor = pepper_sync::wallet::PoolActivation::of(&self.chain_type, protocol)
            .map_or(birthday, |activation| activation.max_with(birthday));
        let shard_ranges = match protocol {
            ShieldedPool::Ironwood => self.sync_state.ironwood_shard_ranges(),
            ShieldedPool::Orchard => self.sync_state.orchard_shard_ranges(),
            ShieldedPool::Sapling => self.sync_state.sapling_shard_ranges(),
        };
        check_note_shards_are_scanned(
            note_height,
            anchor_height,
            scan_floor,
            scan_ranges,
            shard_ranges,
        )
    }
}

fn check_note_shards_are_scanned(
    note_height: Option<BlockHeight>,
    anchor_height: BlockHeight,
    scan_floor: BlockHeight,
    scan_ranges: &[ScanRange],
    shard_ranges: &[Range<BlockHeight>],
) -> bool {
    let incomplete_shard_range = if let Some(shard_range) = shard_ranges.last() {
        shard_range.end - 1..anchor_height + 1
    } else {
        scan_floor..anchor_height + 1
    };
    let mut shard_ranges = shard_ranges.to_vec();
    shard_ranges.push(incomplete_shard_range);

    let mut scanned_ranges = scan_ranges
        .iter()
        .filter(|scan_range| scan_range.priority().is_scanned())
        .cloned()
        .collect::<Vec<_>>();
    'main: loop {
        if scanned_ranges.is_empty() {
            break;
        }
        let mut peekable_ranges = scanned_ranges.iter().enumerate().peekable();
        while let Some((index, range)) = peekable_ranges.next() {
            if let Some((next_index, next_range)) = peekable_ranges.peek() {
                if range.block_range().end == next_range.block_range().start {
                    assert!(*next_index == index + 1);
                    scanned_ranges.splice(
                        index..=*next_index,
                        vec![ScanRange::from_parts(
                            Range {
                                start: range.block_range().start,
                                end: next_range.block_range().end,
                            },
                            ScanPriority::Scanned,
                        )],
                    );
                    continue 'main;
                }
            } else {
                break 'main;
            }
        }
    }

    // a single block may contain two shards at the boundary so we check both are scanned in this case
    note_height
        .into_iter()
        .chain([anchor_height])
        .flat_map(|height| {
            shard_ranges
                .iter()
                .filter(move |&shard_range| shard_range.contains(&height))
        })
        .all(|shard_range| {
            scanned_ranges
                .iter()
                .map(ScanRange::block_range)
                .any(|block_range| {
                    block_range.contains(&(shard_range.end - 1))
                        && (block_range.contains(&shard_range.start)
                            || shard_range.start < scan_floor)
                })
        })
}

#[cfg(test)]
mod proposal_expiry;

#[cfg(test)]
mod tests;
