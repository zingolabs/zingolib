use std::str::FromStr;

use zcash_address::ZcashAddress;
use zcash_client_backend::zip321::TransactionRequest;
use zcash_protocol::PoolType;
use zcash_protocol::memo::{Memo, MemoBytes};
use zcash_protocol::value::Zatoshis;

use crate::lightclient::LightClient;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::synthetic_wallet::{self, SyntheticWalletBuilder};
use crate::wallet::utils;

const ORCHARD_FUNDS: u64 = 100_000;
const PAYMENT: u64 = 20_000;

/// HYPOTHESIS: a wallet that holds Orchard funds alone builds its
/// transaction without parsing the Sapling prover, so a device with no
/// Sapling notes never parses the embedded Sapling parameters.
/// Falsified if the build parses them on this thread.
#[tokio::test]
async fn orchard_only_build_parses_no_sapling_prover() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(ORCHARD_FUNDS)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;
    let proposal = from_inputs::propose(
        &mut client,
        vec![(
            &synthetic_wallet::external_address(PoolType::ORCHARD),
            PAYMENT,
            None,
        )],
    )
    .await
    .expect("an Orchard-only wallet proposes an Orchard payment");

    let parses_before = utils::sapling_prover_parses();
    client
        .wallet()
        .write()
        .await
        .calculate_transactions(proposal, zip32::AccountId::ZERO)
        .await
        .expect("the Orchard-only transaction builds");

    assert_eq!(
        utils::sapling_prover_parses(),
        parses_before,
        "an Orchard-only build parsed the Sapling prover"
    );
}

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
            Some(note_height),
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
            Some(note_height),
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
            Some(note_height),
            anchor_height,
            wallet_birthday,
            &scan_ranges,
            &shard_ranges,
        ));
    }

    // The witness of a note in a scanned shard still needs the leaves of the anchor's shard up to the anchor,
    // so an unscanned block below the anchor in that shard blocks the note.
    #[test]
    fn unscanned_anchor_shard_blocks_a_note_in_a_scanned_shard() {
        let wallet_birthday = BlockHeight::from_u32(10);
        let note_height = BlockHeight::from_u32(70);
        let anchor_height = BlockHeight::from_u32(200);
        let scan_ranges = vec![
            ScanRange::from_parts(wallet_birthday..160.into(), ScanPriority::Scanned),
            ScanRange::from_parts(160.into()..180.into(), ScanPriority::Historic),
            ScanRange::from_parts(180.into()..203.into(), ScanPriority::Scanned),
        ];
        let shard_ranges = vec![
            1.into()..51.into(),
            50.into()..101.into(),
            100.into()..151.into(),
        ];

        assert!(!check_note_shards_are_scanned(
            Some(note_height),
            anchor_height,
            wallet_birthday,
            &scan_ranges,
            &shard_ranges,
        ));
        assert!(!check_note_shards_are_scanned(
            None,
            anchor_height,
            wallet_birthday,
            &scan_ranges,
            &shard_ranges,
        ));
    }

    // Without a note height only the anchor's shard is checked, so an unscanned shard below it is no obstacle.
    #[test]
    fn anchor_shard_alone_is_checked_without_a_note_height() {
        let wallet_birthday = BlockHeight::from_u32(10);
        let anchor_height = BlockHeight::from_u32(200);
        let scan_ranges = vec![
            ScanRange::from_parts(wallet_birthday..60.into(), ScanPriority::Historic),
            ScanRange::from_parts(60.into()..203.into(), ScanPriority::Scanned),
        ];
        let shard_ranges = vec![
            1.into()..51.into(),
            50.into()..101.into(),
            100.into()..151.into(),
        ];

        assert!(check_note_shards_are_scanned(
            None,
            anchor_height,
            wallet_birthday,
            &scan_ranges,
            &shard_ranges,
        ));
        assert!(!check_note_shards_are_scanned(
            Some(BlockHeight::from_u32(20)),
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
            Some(note_height),
            anchor_height,
            wallet_birthday,
            &scan_ranges,
            &shard_ranges,
        ));
    }
}
