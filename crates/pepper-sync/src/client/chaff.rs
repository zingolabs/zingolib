use std::future::Future;

use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::oneshot;
use zcash_primitives::transaction::TxId;
use zl_netutils::lightwallet_protocol::RawTransaction;

pub(crate) const TRIPLE_SIZE: usize = 3;

const TARGET_REPLY_LOST: &str = "the task that requests the triple stopped before the target reply";

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

/// - Spawns a task that sends one request for each transaction of `triple`.
/// - The task outlives this call until every chaff request has its reply.
pub(crate) async fn request<Request>(
    triple: &Triple,
    request_transaction: impl FnMut(TxId) -> Request,
) -> Result<RawTransaction, tonic::Status>
where
    Request: Future<Output = Result<RawTransaction, tonic::Status>> + Send + 'static,
{
    let target = triple.target();
    let mut requests = triple
        .txids()
        .zip(triple.txids().map(request_transaction))
        .map(|(txid, request)| async move { (txid, request.await) })
        .collect::<FuturesUnordered<_>>();
    let (target_reply_sender, target_reply_receiver) = oneshot::channel();
    tokio::spawn(async move {
        let mut target_reply_sender = Some(target_reply_sender);
        while let Some((txid, reply)) = requests.next().await {
            if txid == target
                && let Some(target_reply_sender) = target_reply_sender.take()
            {
                let _ignore_error = target_reply_sender.send(reply);
            }
        }
    });

    target_reply_receiver
        .await
        .unwrap_or_else(|_| Err(tonic::Status::cancelled(TARGET_REPLY_LOST)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tokio::sync::{mpsc, watch};

    use super::*;

    const LONGEST_BLOCK: u8 = 3 * 3 + 2;

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

        let reply = request(&Triple::in_block(&block_txids, target), |txid| {
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
        })
        .await
        .expect("the request for the target answers with a transaction");

        assert_eq!(reply, raw_transaction(target));
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

        let reply = request(
            &Triple::in_block(&block_txids(3), target),
            |txid| async move {
                if txid == target {
                    Ok(raw_transaction(txid))
                } else {
                    Err(tonic::Status::not_found("no such transaction"))
                }
            },
        )
        .await;

        assert!(matches!(reply, Ok(raw_transaction) if raw_transaction.data == target.as_ref()));
    }
}
