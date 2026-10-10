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

        // TODO:  Remove fallible sapling operations from Orchard only sends.
        let (sapling_output, sapling_spend): (Vec<u8>, Vec<u8>) =
            crate::wallet::utils::read_sapling_params();
        let sapling_prover =
            zcash_proofs::prover::LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

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
                step.is_canonical_crossing(&self.pool_migration_params(), canonical_fee)
            })
        });
        (!holds_canonical_crossing).then(|| {
            crate::wallet::expiry::tx_expiry_height(&self.chain_type, target_height.into())
        })
    }

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

    /// Whether every shard carrying `protocol` notes between `note_height` (the scan floor when absent) and `anchor_height` is scanned.
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
            note_height.unwrap_or(scan_floor),
            anchor_height,
            scan_floor,
            scan_ranges,
            shard_ranges,
        )
    }
}

fn check_note_shards_are_scanned(
    note_height: BlockHeight,
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
        .filter(|scan_range| {
            scan_range.priority() == ScanPriority::Scanned
                || scan_range.priority() == ScanPriority::ScannedWithoutMapping
                || scan_range.priority() == ScanPriority::RefetchingNullifiers
        })
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
    shard_ranges
        .iter()
        .filter(|&shard_range| shard_range.contains(&note_height))
        .all(|note_shard_range| {
            scanned_ranges
                .iter()
                .map(ScanRange::block_range)
                .any(|block_range| {
                    block_range.contains(&(note_shard_range.end - 1))
                        && (block_range.contains(&note_shard_range.start)
                            || note_shard_range.start < scan_floor)
                })
        })
}

#[cfg(test)]
mod proposal_expiry {
    use nonempty::NonEmpty;
    use zcash_client_backend::proposal::ShieldedInputs;
    use zcash_client_backend::wallet::{Note, ReceivedNote};
    use zcash_protocol::PoolType;
    use zcash_protocol::consensus::BlockHeight;
    use zcash_protocol::value::{COIN, Zatoshis};

    use crate::mocks::default_txid;
    use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;
    use crate::mocks::proposal::{
        PaymentBuilder, ProposalBuilder, StepBuilder, TransactionRequestBuilder,
    };
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::expiry::tx_expiry_height;
    use crate::wallet::output::OutputRef;
    use pepper_sync::wallet::OutputId;

    const TARGET: u32 = 147;
    /// A ZIP 318 bucket boundary: a multiple of the 144-block grid.
    const BUCKET_BOUNDARY: u32 = 144;

    /// An ordinary proposal takes the wallet's delta for its target height.
    #[test]
    fn an_ordinary_proposal_takes_the_wallets_delta() {
        let wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        let mut builder = ProposalBuilder::default();
        builder.min_target_height(BlockHeight::from_u32(TARGET));
        let proposal = builder.build();

        assert_eq!(
            wallet.proposal_expiry_height(&proposal),
            Some(tx_expiry_height(
                &wallet.chain_type(),
                BlockHeight::from_u32(TARGET)
            ))
        );
    }

    /// A proposal holding a canonical ZIP 318 crossing, one Orchard note
    /// paying exactly one denomination into Ironwood from a bucket-boundary
    /// anchor at the canonical fee, leaves the expiry to the backend, which
    /// gives such a step the ZIP's rolling expiry and refuses any other.
    #[test]
    fn a_canonical_crossing_leaves_the_expiry_to_the_backend() {
        let wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        let chain = wallet.chain_type();
        let canonical_fee = zcash_client_backend::fees::canonical_crossing_fee(
            &chain,
            BlockHeight::from_u32(TARGET),
        )
        .unwrap();
        let denomination = Zatoshis::const_from_u64(COIN);
        let txid = default_txid();
        let note = OrchardCryptoNoteBuilder::default()
            .value(orchard::value::NoteValue::from_raw(
                COIN + u64::from(canonical_fee),
            ))
            .build();
        let mut payment = PaymentBuilder::default();
        payment.amount(denomination);
        let mut request = TransactionRequestBuilder::new();
        request.payments(payment.build());
        let mut step = StepBuilder::default();
        step.transaction_request(request.build())
            .shielded_inputs(Some(ShieldedInputs::from_parts(NonEmpty::singleton(
                ReceivedNote::from_parts(
                    OutputRef::new(OutputId::new(txid, 0), PoolType::ORCHARD),
                    txid,
                    0,
                    Note::Orchard {
                        note,
                        pool: orchard::ValuePool::Orchard,
                    },
                    zip32::Scope::External,
                    incrementalmerkletree::Position::from(1),
                    None,
                    None,
                ),
            ))))
            .anchor_height(BlockHeight::from_u32(BUCKET_BOUNDARY))
            .balance(
                zcash_client_backend::fees::TransactionBalance::new(vec![], canonical_fee).unwrap(),
            );
        let mut builder = ProposalBuilder::default();
        builder
            .min_target_height(BlockHeight::from_u32(TARGET))
            .steps(NonEmpty::singleton(step.build()));
        let proposal = builder.build();

        assert_eq!(wallet.proposal_expiry_height(&proposal), None);
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use zcash_address::ZcashAddress;
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_protocol::memo::{Memo, MemoBytes};
    use zcash_protocol::value::Zatoshis;

    use crate::data::receivers::{Receivers, transaction_request_from_receivers};

    #[test]
    fn test_build_request() {
        let amount_1 = Zatoshis::const_from_u64(20000);
        let recipient_address_1 =
            ZcashAddress::try_from_encoded("utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05").unwrap();
        let memo_1 = None;

        let amount_2 = Zatoshis::const_from_u64(20000);
        let recipient_address_2 =
            ZcashAddress::try_from_encoded("utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05").unwrap();
        let memo_2 = Some(MemoBytes::from(
            Memo::from_str("the lake wavers along the beach").expect("string can memofy"),
        ));

        let rec: Receivers = vec![
            crate::data::receivers::Receiver {
                recipient_address: recipient_address_1,
                amount: amount_1,
                memo: memo_1,
            },
            crate::data::receivers::Receiver {
                recipient_address: recipient_address_2,
                amount: amount_2,
                memo: memo_2,
            },
        ];
        let request: TransactionRequest =
            transaction_request_from_receivers(rec).expect("rec can requestify");

        assert_eq!(
            request.total().expect("total").expect("amounts present"),
            (amount_1 + amount_2).expect("add")
        );
    }

    mod check_note_shards_are_scanned {
        use pepper_sync::sync::{ScanPriority, ScanRange};
        use zcash_protocol::consensus::BlockHeight;

        use crate::wallet::send::check_note_shards_are_scanned;

        #[test]
        fn birthday_within_note_shard_range() {
            let min_confirmations = 3;
            let wallet_birthday = BlockHeight::from_u32(10);
            let last_known_chain_height = BlockHeight::from_u32(202);
            let note_height = BlockHeight::from_u32(20);
            let anchor_height = last_known_chain_height + 1 - min_confirmations;
            let scan_ranges = vec![ScanRange::from_parts(
                wallet_birthday..last_known_chain_height + 1,
                ScanPriority::Scanned,
            )];
            let shard_ranges = vec![
                1.into()..51.into(),
                50.into()..101.into(),
                100.into()..151.into(),
            ];

            assert!(check_note_shards_are_scanned(
                note_height,
                anchor_height,
                wallet_birthday,
                &scan_ranges,
                &shard_ranges,
            ));
        }

        #[test]
        fn note_within_complete_shard() {
            let min_confirmations = 3;
            let wallet_birthday = BlockHeight::from_u32(10);
            let last_known_chain_height = BlockHeight::from_u32(202);
            let note_height = BlockHeight::from_u32(70);
            let anchor_height = last_known_chain_height + 1 - min_confirmations;
            let scan_ranges = vec![ScanRange::from_parts(
                wallet_birthday..last_known_chain_height + 1,
                ScanPriority::Scanned,
            )];
            let shard_ranges = vec![
                1.into()..51.into(),
                50.into()..101.into(),
                100.into()..151.into(),
            ];

            assert!(check_note_shards_are_scanned(
                note_height,
                anchor_height,
                wallet_birthday,
                &scan_ranges,
                &shard_ranges,
            ));
        }

        #[test]
        fn note_within_incomplete_shard() {
            let min_confirmations = 3;
            let wallet_birthday = BlockHeight::from_u32(10);
            let last_known_chain_height = BlockHeight::from_u32(202);
            let note_height = BlockHeight::from_u32(170);
            let anchor_height = last_known_chain_height + 1 - min_confirmations;
            let scan_ranges = vec![ScanRange::from_parts(
                wallet_birthday..last_known_chain_height + 1,
                ScanPriority::Scanned,
            )];
            let shard_ranges = vec![
                1.into()..51.into(),
                50.into()..101.into(),
                100.into()..151.into(),
            ];

            assert!(check_note_shards_are_scanned(
                note_height,
                anchor_height,
                wallet_birthday,
                &scan_ranges,
                &shard_ranges,
            ));
        }

        #[test]
        fn note_height_on_shard_boundary() {
            let min_confirmations = 3;
            let wallet_birthday = BlockHeight::from_u32(10);
            let last_known_chain_height = BlockHeight::from_u32(202);
            let note_height = BlockHeight::from_u32(100);
            let anchor_height = last_known_chain_height + 1 - min_confirmations;
            let scan_ranges = vec![ScanRange::from_parts(
                wallet_birthday..last_known_chain_height + 1,
                ScanPriority::Scanned,
            )];
            let shard_ranges = vec![
                1.into()..51.into(),
                50.into()..101.into(),
                100.into()..151.into(),
            ];

            assert!(check_note_shards_are_scanned(
                note_height,
                anchor_height,
                wallet_birthday,
                &scan_ranges,
                &shard_ranges,
            ));
        }
    }
}
