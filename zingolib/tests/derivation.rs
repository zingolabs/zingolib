#![forbid(unsafe_code)]
#![cfg(all(feature = "perspective", feature = "testutils"))]
//! Unit tests of the value-transfer derivation over fabricated wallet
//! transaction records.

#[path = "support/common.rs"]
mod common;

use pepper_sync::wallet::{IronwoodNote, OutputId, WalletTransaction};
use zcash_primitives::transaction::TxId;
use zcash_protocol::memo::Memo;
use zingo_common_components::protocol::ActivationHeights;
use zingo_status::confirmation_status::ConfirmationStatus;
use zingo_test_vectors::seeds;
use zingolib::ZENNIES_FOR_ZINGO_REGTEST_ADDRESS;
use zingolib::config::ChainType;
use zingolib::mocks::orchard_note::OrchardCryptoNoteBuilder;
use zingolib::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind, ValueTransfers,
};
use zingolib::wallet::keys::unified::ReceiverSelection;

use common::{
    own_orchard_receiver, received_texts, regtest_wallet, self_send, sent, zfz_self_send,
};

/// Migrated from libtonode `fast::filter_empty_messages`.
#[tokio::test]
async fn filter_empty_messages() {
    let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

    // Two received notes with empty memos: no messages.
    wallet
        .wallet_transactions
        .insert(TxId::from_bytes([1; 32]), received_texts(1, 10, &["", ""]));
    assert_eq!(wallet.messages_containing(None).await.unwrap().len(), 0);

    // One real memo alongside an empty one: exactly one message.
    wallet.wallet_transactions.insert(
        TxId::from_bytes([2; 32]),
        received_texts(2, 11, &["Hello", ""]),
    );
    assert_eq!(wallet.messages_containing(None).await.unwrap().len(), 1);
}

/// Migrated from libtonode `fast::message_thread`.
#[tokio::test]
async fn message_thread() {
    // Alice is this wallet; Bob and Charlie are addresses of a foreign
    // wallet (different seed), exactly as the integration test used the
    // faucet's addresses.
    let mut alice_wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
    let network = ChainType::Regtest(ActivationHeights::default());
    let alice = alice_wallet
        .unified_addresses()
        .values()
        .next()
        .unwrap()
        .encode(&network);

    let mut other_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
    let (_, bob) = other_wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();
    let (_, charlie) = other_wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();
    let bob_encoded = bob.encode(&network);
    let charlie_encoded = charlie.encode(&network);

    for transaction in [
        sent(1, 10, &bob, &format!("Alice->Bob #1\nReply to\n{alice}")),
        sent(2, 11, &bob, &format!("Alice->Bob #2\nReply to\n{alice}")),
        received_texts(3, 12, &[&format!("Bob->Alice #2\nReply to\n{bob_encoded}")]),
        sent(
            4,
            13,
            &charlie,
            &format!("Alice->Charlie #2\nReply to\n{alice}"),
        ),
        received_texts(
            5,
            14,
            &[&format!("Charlie->Alice #2\nReply to\n{charlie_encoded}")],
        ),
    ] {
        alice_wallet
            .wallet_transactions
            .insert(transaction.txid(), transaction);
    }

    let messages_bob = alice_wallet
        .messages_containing(Some(&bob_encoded))
        .await
        .unwrap();
    let messages_charlie = alice_wallet
        .messages_containing(Some(&charlie_encoded))
        .await
        .unwrap();
    let all_vts = alice_wallet.value_transfers(true).await.unwrap();
    let all_messages = alice_wallet.messages_containing(None).await.unwrap();

    assert_eq!(messages_bob.len(), 3);
    assert_eq!(messages_charlie.len(), 2);

    // ALL MESSAGES (first one should be the oldest one)
    assert!(
        all_messages
            .windows(2)
            .all(|pair| pair[0].blockheight <= pair[1].blockheight)
    );
    // ALL VTS (first one should be the most recent one)
    assert!(
        all_vts
            .windows(2)
            .all(|pair| pair[0].blockheight >= pair[1].blockheight)
    );
}

/// Migrated from libtonode `fast::create_send_to_self_with_zfz_active`:
/// a self-send yields SendToSelf(Basic) and the Zennies-for-Zingo output
/// yields a Sent(Send) addressed to the ZFZ address.
#[tokio::test]
async fn create_send_to_self_with_zfz_active() {
    let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

    let transaction = zfz_self_send(1, 10, own_orchard_receiver(&wallet));
    wallet
        .wallet_transactions
        .insert(transaction.txid(), transaction);

    let value_transfers = wallet.value_transfers(true).await.unwrap();

    assert!(value_transfers.iter().any(|vt| vt.kind
        == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Basic))));
    assert!(value_transfers.iter().any(|vt| vt.kind
        == ValueTransferKind::Sent(SentValueTransfer::Send)
        && vt.recipient_address == Some(ZENNIES_FOR_ZINGO_REGTEST_ADDRESS.to_string())));
}

/// A send value transfer exposes the pools the recipient's outputs
/// landed in, letting consumers label which pool a payment was
/// delivered to. The funding side (`pools_sent_from`) needs real spend
/// links, so it is pinned by the chain-bound tests in libtonode.
#[tokio::test]
async fn send_exposes_recipient_pools_received() {
    use zcash_protocol::PoolType;

    let mut alice_wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
    let mut other_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
    let (_, bob) = other_wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();

    alice_wallet
        .wallet_transactions
        .insert(TxId::from_bytes([1; 32]), sent(1, 10, &bob, "hi Bob"));

    let value_transfers = alice_wallet.value_transfers(true).await.unwrap();

    let send = value_transfers
        .iter()
        .find(|vt| vt.kind == ValueTransferKind::Sent(SentValueTransfer::Send))
        .unwrap();
    assert_eq!(send.pools_received, [PoolType::IRONWOOD]);
    assert!(send.pools_sent_from.is_empty());
}

/// A send-to-self value transfer exposes the pools its value arrived
/// into (here: ironwood), letting consumers label pool movements such
/// as an orchard -> ironwood migration outside zingolib.
#[tokio::test]
async fn send_to_self_exposes_pools_received() {
    use zcash_protocol::PoolType;

    let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

    let transaction = self_send(1, 10, own_orchard_receiver(&wallet));
    wallet
        .wallet_transactions
        .insert(transaction.txid(), transaction);

    let value_transfers = wallet.value_transfers(true).await.unwrap();

    let self_send = value_transfers
        .iter()
        .find(|vt| {
            vt.kind
                == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                    SelfSendValueTransfer::Basic,
                ))
        })
        .unwrap();
    assert_eq!(self_send.pools_received, [PoolType::IRONWOOD]);
    assert!(self_send.pools_sent_from.is_empty());
}

/// Migrated from libtonode `slow::by_address_finsight`: the
/// memo-bytes-per-address summary accumulates outgoing memo lengths
/// keyed by recipient address. Two 1-byte memos then a 4-byte memo.
#[tokio::test]
async fn by_address_finsight() {
    let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

    let mut external_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
    let (_, external_ua) = external_wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();
    let external_ua_encoded = external_ua.encode(&external_wallet.chain_type());

    for (txid_byte, height, memo) in [(1, 4, "1"), (2, 5, "1")] {
        let transaction = sent(txid_byte, height, &external_ua, memo);
        wallet
            .wallet_transactions
            .insert(transaction.txid(), transaction);
    }
    let memobytes = wallet.do_total_memobytes_to_address().await.unwrap();
    assert_eq!(
        json::JsonValue::from(memobytes)[&external_ua_encoded].pretty(4),
        "2".to_string()
    );

    let transaction = sent(3, 6, &external_ua, "aaaa");
    wallet
        .wallet_transactions
        .insert(transaction.txid(), transaction);
    let memobytes = wallet.do_total_memobytes_to_address().await.unwrap();
    assert_eq!(
        json::JsonValue::from(memobytes)[&external_ua_encoded].pretty(4),
        "6".to_string()
    );
}

/// Migrated from libtonode `fast::value_transfers`: a four-output memo'd
/// receive aggregates into one value transfer, the derivation is
/// idempotent, and the descending sort reverses the ascending one at
/// transaction granularity while preserving intra-transaction creation
/// order.
#[tokio::test]
async fn value_transfers_aggregation_and_ordering() {
    use std::str::FromStr as _;

    use incrementalmerkletree::Position;
    use orchard::value::NoteValue;

    let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

    // One received transaction carrying four memo'd notes...
    let txid = TxId::from_bytes([1; 32]);
    let notes = (0..4u64)
        .map(|index| {
            IronwoodNote::new_for_test(
                OutputId::new(txid, u32::try_from(index).unwrap()),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default()
                    .value(NoteValue::from_raw(5_000))
                    .build(),
                Memo::from_str(&format!("Message #{}", index + 1)).unwrap(),
                Some(Position::from(index)),
            )
        })
        .collect::<Vec<_>>();
    wallet.wallet_transactions.insert(
        txid,
        WalletTransaction::new_for_test_with_ironwood_notes(
            txid,
            ConfirmationStatus::Confirmed(4.into()),
            notes,
            vec![],
        ),
    );
    // ...plus two single-note receives at later heights so the
    // ordering assertions exercise a non-trivial sort.
    for (txid_byte, height) in [(2u8, 5u32), (3, 6)] {
        let txid = TxId::from_bytes([txid_byte; 32]);
        let note = IronwoodNote::new_for_test(
            OutputId::new(txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            OrchardCryptoNoteBuilder::default()
                .value(NoteValue::from_raw(1_000 * u64::from(txid_byte)))
                .build(),
            Memo::Empty,
            Some(Position::from(u64::from(txid_byte))),
        );
        wallet.wallet_transactions.insert(
            txid,
            WalletTransaction::new_for_test_with_ironwood_notes(
                txid,
                ConfirmationStatus::Confirmed(height.into()),
                vec![note],
                vec![],
            ),
        );
    }
    // ...plus one transaction yielding TWO value transfers (a self-send
    // and a ZFZ send), so the intra-transaction assertions bite.
    let zfz_txid = TxId::from_bytes([4; 32]);
    let transaction = zfz_self_send(4, 7, own_orchard_receiver(&wallet));
    wallet
        .wallet_transactions
        .insert(transaction.txid(), transaction);

    let descending = wallet.value_transfers(true).await.unwrap();
    let ascending = wallet.value_transfers(false).await.unwrap();

    let four_memo_transfer = descending
        .iter()
        .find(|transfer| transfer.txid == TxId::from_bytes([1; 32]))
        .unwrap();
    assert_eq!(four_memo_transfer.memos.len(), 4);

    // Idempotence.
    assert_eq!(descending, wallet.value_transfers(true).await.unwrap());
    assert_eq!(ascending, wallet.value_transfers(false).await.unwrap());

    // Descending reverses ascending at transaction granularity only: the
    // per-transaction blocks reverse, while each transaction's transfers
    // keep their creation order in both directions.
    let transaction_blocks = |value_transfers: &ValueTransfers| {
        let mut blocks: Vec<TxId> = Vec::new();
        for value_transfer in value_transfers.iter() {
            if blocks.last() != Some(&value_transfer.txid) {
                blocks.push(value_transfer.txid);
            }
        }
        blocks
    };
    let mut reversed_ascending_blocks = transaction_blocks(&ascending);
    reversed_ascending_blocks.reverse();
    assert_eq!(transaction_blocks(&descending), reversed_ascending_blocks);

    let kinds_of = |value_transfers: &ValueTransfers, txid: TxId| {
        value_transfers
            .iter()
            .filter(|value_transfer| value_transfer.txid == txid)
            .map(|value_transfer| value_transfer.kind)
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds_of(&descending, zfz_txid).len(), 2);
    for txid in transaction_blocks(&descending) {
        assert_eq!(kinds_of(&descending, txid), kinds_of(&ascending, txid));
    }
}
