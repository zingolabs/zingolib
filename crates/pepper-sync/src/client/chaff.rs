use std::{collections::HashMap, future::Future};

use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::{mpsc::UnboundedReceiver, oneshot};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus;
use zip32::AccountId;
use zl_netutils::lightwallet_protocol::RawTransaction;

pub(crate) const TRIPLE_SIZE: usize = 3;
pub(crate) const TARGET_FREE_BLOCK_INTERVAL: u64 = 1_024;

const SELECTOR_KEY_BYTES: usize = 32;
const SELECTION_BYTES: usize = 2 * size_of::<u64>();
const SELECTOR_KEY_PERSONALIZATION: &[u8; blake2b_simd::PERSONALBYTES] = b"zl_chaff_sel_key";
const SELECTION_PERSONALIZATION: &[u8; blake2b_simd::PERSONALBYTES] = b"zl_chaff_select_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Triple {
    before: Vec<TxId>,
    target: TxId,
    after: Vec<TxId>,
}

impl Triple {
    pub(crate) fn alone(target: TxId) -> Self {
        Self {
            before: Vec::new(),
            target,
            after: Vec::new(),
        }
    }

    pub(crate) fn in_block(block_txids: &[TxId], target: TxId) -> Self {
        let Some(position) = block_txids.iter().position(|txid| *txid == target) else {
            return Self::alone(target);
        };
        let last_triple = (block_txids.len() / TRIPLE_SIZE).saturating_sub(1);
        let triple = (position / TRIPLE_SIZE).min(last_triple);
        let start = triple * TRIPLE_SIZE;
        let end = if triple == last_triple {
            block_txids.len()
        } else {
            start + TRIPLE_SIZE
        };

        Self {
            before: block_txids[start..position].to_vec(),
            target,
            after: block_txids[position + 1..end].to_vec(),
        }
    }

    pub(crate) fn target(&self) -> TxId {
        self.target
    }

    pub(crate) fn txids(&self) -> impl Iterator<Item = TxId> + '_ {
        self.before
            .iter()
            .copied()
            .chain([self.target])
            .chain(self.after.iter().copied())
    }
}

pub(crate) struct TargetFreeSelector {
    key: [u8; SELECTOR_KEY_BYTES],
}

impl TargetFreeSelector {
    pub(crate) fn new(
        consensus_parameters: &impl consensus::Parameters,
        ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    ) -> Option<Self> {
        let (_, ufvk) = ufvks.iter().min_by_key(|(account_id, _)| **account_id)?;
        let key = blake2b_simd::Params::new()
            .hash_length(SELECTOR_KEY_BYTES)
            .personal(SELECTOR_KEY_PERSONALIZATION)
            .hash(ufvk.encode(consensus_parameters).ok()?.as_bytes());

        Some(Self {
            key: *key.as_bytes().first_chunk()?,
        })
    }

    pub(crate) fn triple_in(&self, block_hash: &[u8], block_txids: &[TxId]) -> Option<Triple> {
        let selection = blake2b_simd::Params::new()
            .hash_length(SELECTION_BYTES)
            .key(&self.key)
            .personal(SELECTION_PERSONALIZATION)
            .hash(block_hash);
        let block_draw = u64::from_le_bytes(*selection.as_bytes().first_chunk()?);
        let position_draw = u64::from_le_bytes(*selection.as_bytes().last_chunk()?);
        if block_draw % TARGET_FREE_BLOCK_INTERVAL != 0 {
            return None;
        }
        let position =
            usize::try_from(position_draw.checked_rem(u64::try_from(block_txids.len()).ok()?)?)
                .ok()?;

        Some(Triple::in_block(block_txids, *block_txids.get(position)?))
    }
}

pub(crate) type TargetReplySender = oneshot::Sender<Result<RawTransaction, tonic::Status>>;

pub(crate) struct TripleRequest {
    pub(crate) triple: Triple,
    pub(crate) target_reply_sender: Option<TargetReplySender>,
}

/// - Sends one request for each transaction of every triple that `triple_requests` yields.
/// - Sends the reply for the target of a triple to its reply sender, if the triple has one.
/// - Awaits every reply of one triple before it takes the next triple.
pub(crate) async fn request_triples<Request>(
    mut triple_requests: UnboundedReceiver<TripleRequest>,
    mut request_transaction: impl FnMut(TxId) -> Request,
) where
    Request: Future<Output = Result<RawTransaction, tonic::Status>>,
{
    while let Some(TripleRequest {
        triple,
        mut target_reply_sender,
    }) = triple_requests.recv().await
    {
        let mut requests = triple
            .txids()
            .zip(triple.txids().map(&mut request_transaction))
            .map(|(txid, request)| async move { (txid, request.await) })
            .collect::<FuturesUnordered<_>>();
        while let Some((txid, reply)) = requests.next().await {
            if txid == triple.target()
                && let Some(target_reply_sender) = target_reply_sender.take()
            {
                let _ignore_error = target_reply_sender.send(reply);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tokio::sync::{mpsc, watch};
    use zcash_keys::keys::UnifiedSpendingKey;
    use zcash_protocol::consensus::MAIN_NETWORK;

    use super::*;

    const LONGEST_BLOCK: u8 = 3 * 3 + 2;
    const EXPECTED_SELECTED_BLOCKS: u32 = 64;
    const SAMPLED_BLOCKS: u32 = EXPECTED_SELECTED_BLOCKS * 1_024;
    const PINNED_SELECTED_BLOCKS: usize = 63;
    const PINNED_FIRST_SELECTED_BLOCK: u32 = 3_502;

    fn selector(seed_byte: u8) -> TargetFreeSelector {
        let ufvk = UnifiedSpendingKey::from_seed(&MAIN_NETWORK, &[seed_byte; 32], AccountId::ZERO)
            .expect("a seed of 32 bytes derives the spending key of account zero")
            .to_unified_full_viewing_key();

        TargetFreeSelector::new(&MAIN_NETWORK, &HashMap::from([(AccountId::ZERO, ufvk)]))
            .expect("a wallet with one viewing key has a selector")
    }

    fn block_hash(index: u32) -> [u8; 32] {
        let mut block_hash = [0; 32];
        block_hash[..size_of::<u32>()].copy_from_slice(&index.to_le_bytes());
        block_hash
    }

    fn selected_blocks(selector: &TargetFreeSelector) -> Vec<u32> {
        let block_txids = block_txids(LONGEST_BLOCK);
        (0..SAMPLED_BLOCKS)
            .filter(|index| {
                selector
                    .triple_in(&block_hash(*index), &block_txids)
                    .is_some()
            })
            .collect()
    }

    #[test]
    fn a_wallet_selects_the_same_blocks_every_time_and_another_wallet_selects_others() {
        let selected = selected_blocks(&selector(0));

        assert_eq!(selected, selected_blocks(&selector(0)));
        assert_ne!(selected, selected_blocks(&selector(1)));
    }

    #[test]
    fn about_one_block_in_the_interval_is_selected() {
        assert_eq!(u64::from(SAMPLED_BLOCKS) / TARGET_FREE_BLOCK_INTERVAL, 64);
        for seed_byte in 0..4 {
            let selected = selected_blocks(&selector(seed_byte)).len();
            assert!(
                (EXPECTED_SELECTED_BLOCKS as usize / 2..=EXPECTED_SELECTED_BLOCKS as usize * 2)
                    .contains(&selected),
                "seed {seed_byte} selected {selected} blocks"
            );
        }
    }

    #[test]
    fn the_selection_of_a_known_wallet_is_pinned() {
        let selected = selected_blocks(&selector(0));

        assert_eq!(
            (selected.len(), selected.first().copied()),
            (PINNED_SELECTED_BLOCKS, Some(PINNED_FIRST_SELECTED_BLOCK))
        );
    }

    #[test]
    fn a_selected_block_gives_one_of_its_triples_and_an_empty_block_gives_none() {
        let selector = selector(0);
        let block_txids = block_txids(LONGEST_BLOCK);
        let selected = selected_blocks(&selector);
        assert!(!selected.is_empty());

        for index in selected {
            let triple = selector
                .triple_in(&block_hash(index), &block_txids)
                .expect("the block was selected with the same transactions");
            assert_eq!(triple, Triple::in_block(&block_txids, triple.target()));
            assert_eq!(selector.triple_in(&block_hash(index), &[]), None);
        }
    }

    #[test]
    fn a_wallet_without_a_viewing_key_has_no_selector() {
        assert!(TargetFreeSelector::new(&MAIN_NETWORK, &HashMap::new()).is_none());
    }

    #[tokio::test]
    async fn a_triple_with_a_target_and_a_triple_without_one_are_requested_alike() {
        #[derive(Debug, PartialEq)]
        enum Event {
            Sent(TxId),
            Answered(TxId),
        }
        let first_triple = Triple::in_block(&block_txids(3), txid(0));
        let second_triple = Triple::in_block(&block_txids(6), txid(5));
        let (triple_request_sender, triple_request_receiver) = mpsc::unbounded_channel();
        let (event_log, mut events) = mpsc::unbounded_channel();
        let (target_reply_sender, target_reply) = oneshot::channel();
        for (triple, target_reply_sender) in [
            (&first_triple, Some(target_reply_sender)),
            (&second_triple, None),
        ] {
            assert!(
                triple_request_sender
                    .send(TripleRequest {
                        triple: triple.clone(),
                        target_reply_sender,
                    })
                    .is_ok()
            );
        }
        drop(triple_request_sender);

        request_triples(triple_request_receiver, |txid| {
            let event_log = event_log.clone();
            async move {
                event_log
                    .send(Event::Sent(txid))
                    .expect("the test holds the receiver of the event log");
                tokio::task::yield_now().await;
                event_log
                    .send(Event::Answered(txid))
                    .expect("the test holds the receiver of the event log");
                Ok(raw_transaction(txid))
            }
        })
        .await;

        let mut logged = Vec::new();
        while let Ok(event) = events.try_recv() {
            logged.push(event);
        }
        let mut logged = logged.as_slice();
        for triple in [&first_triple, &second_triple] {
            let triple_size = triple.txids().count();
            let (sent, rest) = logged.split_at(triple_size);
            let (answered, rest) = rest.split_at(triple_size);
            assert_eq!(sent, triple.txids().map(Event::Sent).collect::<Vec<_>>());
            for txid in triple.txids() {
                assert!(answered.contains(&Event::Answered(txid)));
            }
            logged = rest;
        }
        assert!(logged.is_empty());
        assert_eq!(
            target_reply.await.map(Result::ok),
            Ok(Some(raw_transaction(first_triple.target())))
        );
    }

    fn txid(index: u8) -> TxId {
        TxId::from_bytes([index; 32])
    }

    fn block_txids(transaction_count: u8) -> Vec<TxId> {
        (0..transaction_count).map(txid).collect()
    }

    fn raw_transaction(txid: TxId) -> RawTransaction {
        RawTransaction {
            data: txid.as_ref().to_vec(),
            height: 0,
        }
    }

    #[test]
    fn every_member_of_a_triple_requests_the_same_transactions() {
        for transaction_count in 0..=LONGEST_BLOCK {
            let block_txids = block_txids(transaction_count);
            for target in &block_txids {
                let requested = Triple::in_block(&block_txids, *target)
                    .txids()
                    .collect::<Vec<_>>();
                assert!(requested.contains(target));
                for member in &requested {
                    assert_eq!(
                        Triple::in_block(&block_txids, *member)
                            .txids()
                            .collect::<Vec<_>>(),
                        requested
                    );
                }
            }
        }
    }

    #[test]
    fn a_block_of_three_or_more_gives_every_target_two_or_more_chaff() {
        for transaction_count in 0..=LONGEST_BLOCK {
            let block_txids = block_txids(transaction_count);
            let block_size = block_txids.len();
            let smallest_request = TRIPLE_SIZE.min(block_size);
            let largest_request = (2 * TRIPLE_SIZE - 1).min(block_size);
            for target in &block_txids {
                let request_size = Triple::in_block(&block_txids, *target).txids().count();
                assert!(
                    (smallest_request..=largest_request).contains(&request_size),
                    "a block of {block_size} gave a request of {request_size}"
                );
            }
        }
    }

    #[test]
    fn triples_follow_block_order_and_the_short_tail_joins_the_last_triple() {
        let block_txids = block_txids(7);

        assert_eq!(
            Triple::in_block(&block_txids, txid(1))
                .txids()
                .collect::<Vec<_>>(),
            block_txids[..3]
        );
        assert_eq!(
            Triple::in_block(&block_txids, txid(6))
                .txids()
                .collect::<Vec<_>>(),
            block_txids[3..]
        );
    }

    #[test]
    fn a_target_the_block_does_not_hold_is_requested_alone() {
        assert_eq!(
            Triple::in_block(&block_txids(6), txid(9)),
            Triple::alone(txid(9))
        );
    }

    #[tokio::test]
    async fn the_target_reply_returns_while_chaff_replies_are_outstanding() {
        let block_txids = block_txids(3);
        let target = txid(1);
        let (release_chaff, chaff_released) = watch::channel(false);
        let (request_log, mut requested) = mpsc::unbounded_channel();
        let (reply_log, mut replied) = mpsc::unbounded_channel();
        let (triple_request_sender, triple_request_receiver) = mpsc::unbounded_channel();
        let (target_reply_sender, target_reply) = oneshot::channel();
        assert!(
            triple_request_sender
                .send(TripleRequest {
                    triple: Triple::in_block(&block_txids, target),
                    target_reply_sender: Some(target_reply_sender),
                })
                .is_ok()
        );

        tokio::spawn(request_triples(triple_request_receiver, move |txid| {
            let mut chaff_released = chaff_released.clone();
            let request_log = request_log.clone();
            let reply_log = reply_log.clone();
            async move {
                request_log
                    .send(txid)
                    .expect("the test holds the receiver of the request log");
                if txid != target {
                    chaff_released
                        .wait_for(|released| *released)
                        .await
                        .expect("the test holds the sender that releases the chaff");
                }
                reply_log
                    .send(txid)
                    .expect("the test holds the receiver of the reply log");
                Ok(raw_transaction(txid))
            }
        }));
        let reply = target_reply
            .await
            .expect("the worker holds the reply sender until the target reply arrives");

        assert_eq!(reply.ok(), Some(raw_transaction(target)));
        let mut requested_in_order = Vec::new();
        while let Ok(txid) = requested.try_recv() {
            requested_in_order.push(txid);
        }
        assert_eq!(requested_in_order, block_txids);
        assert_eq!(replied.try_recv(), Ok(target));
        assert!(replied.try_recv().is_err());

        release_chaff
            .send(true)
            .expect("each chaff request holds a receiver");
        let chaff_replies = BTreeSet::from([
            replied
                .recv()
                .await
                .expect("the first chaff request completes"),
            replied
                .recv()
                .await
                .expect("the second chaff request completes"),
        ]);
        assert_eq!(chaff_replies, BTreeSet::from([txid(0), txid(2)]));
    }

    #[tokio::test]
    async fn a_failed_chaff_request_does_not_fail_the_target() {
        let target = txid(1);

        let (triple_request_sender, triple_request_receiver) = mpsc::unbounded_channel();
        let (target_reply_sender, target_reply) = oneshot::channel();
        assert!(
            triple_request_sender
                .send(TripleRequest {
                    triple: Triple::in_block(&block_txids(3), target),
                    target_reply_sender: Some(target_reply_sender),
                })
                .is_ok()
        );
        drop(triple_request_sender);

        request_triples(triple_request_receiver, |txid| async move {
            if txid == target {
                Ok(raw_transaction(txid))
            } else {
                Err(tonic::Status::not_found("no such transaction"))
            }
        })
        .await;

        assert_eq!(
            target_reply.await.map(Result::ok),
            Ok(Some(raw_transaction(target)))
        );
    }
}
