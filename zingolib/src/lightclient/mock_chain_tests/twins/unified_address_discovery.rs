use zcash_client_backend::encoding::encode_payment_address_p;
use zip32::AccountId;

use super::HOSPITAL_MUSEUM;
use crate::lightclient::LightClient;
use crate::testutils::mock_indexer::{MockNet, faucet_funding_transaction};
use crate::wallet::keys::unified::{ReceiverSelection, UnifiedAddressId};

const ORCHARD_ONLY_INDEX: u32 = 2;
const SAPLING_ONLY_INDEX: u32 = 3;
const ALL_SHIELDED_INDEX: u32 = 4;

async fn encoded_address_at(client: &LightClient, address_index: u32) -> Option<String> {
    let network = client.chain_type();
    client
        .wallet()
        .read()
        .await
        .unified_addresses()
        .get(&UnifiedAddressId {
            account_id: AccountId::ZERO,
            address_index,
        })
        .map(|address| address.encode(&network))
}

#[tokio::test]
async fn unified_address_discovery() {
    let mut net = MockNet::launch().await;
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    let network = recipient.chain_type();

    // create a range of UAs to be discovered when recipient is reset
    let orchard_only_addr = recipient
        .generate_unified_address(ReceiverSelection::orchard_only(), AccountId::ZERO)
        .await
        .map(|(_, ua)| ua.encode(&network))
        .expect("a seed wallet derives an orchard-only address");
    let sapling_only_addr = recipient
        .generate_unified_address(ReceiverSelection::sapling_only(), AccountId::ZERO)
        .await
        .map(|(_, ua)| ua.encode(&network))
        .expect("a seed wallet derives a sapling-only address");
    let (_, all_shielded_addr) = recipient
        .generate_unified_address(ReceiverSelection::all_shielded(), AccountId::ZERO)
        .await
        .expect("a seed wallet derives an all-shielded address");
    let all_shielded_encoded = all_shielded_addr.encode(&network);
    let all_shielded_sapling_addr = all_shielded_addr
        .sapling()
        .map(|addr| encode_payment_address_p(&network, addr))
        .expect("an all-shielded address carries a sapling receiver");

    // send to the UAs so they are recorded on chain
    net.chain.write().await.mine_empty_blocks(1);
    let funding = faucet_funding_transaction(vec![
        (&orchard_only_addr, 100_000, Some("orchard_only")),
        (&sapling_only_addr, 200_000, Some("sapling_only")),
        (&all_shielded_encoded, 300_000, Some("all_shielded")),
        (
            &all_shielded_sapling_addr,
            400_000,
            Some("all_shielded_sapling"),
        ),
    ])
    .await;
    net.chain.write().await.mine_block(vec![funding]);

    // rebuild recipient and check the UAs don't exist in the wallet
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    for address_index in [ORCHARD_ONLY_INDEX, SAPLING_ONLY_INDEX, ALL_SHIELDED_INDEX] {
        assert!(
            encoded_address_at(&recipient, address_index)
                .await
                .is_none(),
            "address {address_index} should not be in a fresh wallet yet"
        );
    }

    // sync recipient and check the UAs have been discovered
    recipient
        .sync_and_await()
        .await
        .expect("the rebuilt wallet syncs the funding block");
    assert_eq!(
        encoded_address_at(&recipient, ORCHARD_ONLY_INDEX).await,
        Some(orchard_only_addr)
    );
    assert_eq!(
        encoded_address_at(&recipient, SAPLING_ONLY_INDEX).await,
        Some(sapling_only_addr)
    );
    assert_eq!(
        encoded_address_at(&recipient, ALL_SHIELDED_INDEX).await,
        Some(all_shielded_encoded)
    );
}
