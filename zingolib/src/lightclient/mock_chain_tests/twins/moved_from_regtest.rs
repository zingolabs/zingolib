use std::time::Duration;

use zcash_protocol::PoolType;
use zcash_protocol::consensus::COINBASE_MATURITY_BLOCKS;
use zcash_protocol::value::Zatoshis;
use zip32::AccountId;

use zcash_protocol::ShieldedPool;

use super::{DEFERRED_NU6_3, FUNDING, HOSPITAL_MUSEUM, external_address, fund};
use crate::check_client_balances;
use crate::testutils::fee_tables;
use crate::testutils::lightclient::{from_inputs, get_base_address, get_fees_paid_by_client};
use crate::testutils::mock_activation_heights_deferring_nu6_3;
use crate::testutils::mock_indexer::{MockChain, MockNet};

const REWARD_ZATS: u64 = 625_000;
const REWARD: Zatoshis = Zatoshis::const_from_u64(REWARD_ZATS);
const COINBASES: usize = 4;

#[tokio::test]
async fn mine_to_transparent_and_shield() {
    let mut net = MockNet::launch().await;
    let mut miner = net.client(HOSPITAL_MUSEUM, None).await;
    miner.set_transmit_retry_interval(Duration::ZERO);
    let miner_taddr = get_base_address(&miner, PoolType::Transparent).await;
    {
        let mut chain = net.chain.write().await;
        for _ in 0..COINBASES {
            chain.mine_block_rewarding(&miner_taddr, REWARD, vec![]);
        }
        chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
    }
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    let coinbase_total = (REWARD * COINBASES)
        .expect("four rewards stay within the money range")
        .into_u64();
    check_client_balances!(miner, i: 0 o: 0 s: 0 t: coinbase_total);

    miner
        .quick_shield(AccountId::ZERO)
        .await
        .expect("four matured coinbases shield in one transaction");
    net.chain.write().await.mine_mempool();
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");

    let fee = get_fees_paid_by_client(&miner).await;
    assert_eq!(fee, fee_tables::shield(COINBASES));
    check_client_balances!(miner, i: (coinbase_total - fee) o: 0 s: 0 t: 0);
}
#[tokio::test]
async fn sapling_dust_fee_collection() {
    const FEES_FOR_ORCHARD: u64 = 10;
    const DUST_DIVISOR: u64 = 10;
    const FEES_SENT: u64 = 5;
    let fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::ORCHARD, true);
    let for_orchard = fee * FEES_FOR_ORCHARD;
    let for_sapling = fee / DUST_DIVISOR;

    let mut net = MockNet::launch().await;
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    let recipient_sapling =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Sapling)).await;
    let recipient_unified =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    check_client_balances!(recipient, i: 0 o: 0 s: 0 t: 0);

    net.chain.write().await.mine_empty_blocks(1);
    fund(
        &net,
        vec![
            (&recipient_unified, for_orchard, Some("Plenty for orchard.")),
            (&recipient_sapling, for_sapling, Some("Dust for sapling.")),
        ],
        0,
    )
    .await;
    recipient
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    check_client_balances!(recipient, i: for_orchard o: 0 s: 0 t: 0);

    from_inputs::quick_send(
        &mut recipient,
        vec![(
            &external_address(PoolType::ORCHARD),
            fee * FEES_SENT,
            Some("Five times fee."),
        )],
    )
    .await
    .expect("the funded wallet covers the payment and its fee");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    let remaining_ironwood = for_orchard - (FEES_SENT + 1) * fee;
    check_client_balances!(recipient, i: remaining_ironwood o: 0 s: 0 t: 0);
}
#[tokio::test]
async fn send_and_sync_with_multiple_notes_no_panic() {
    const NOTE: u64 = 40_000;
    const PAYMENT: u64 = 50_000;
    let fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::ORCHARD, true);

    let mut net = MockNet::launch().await;
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, NOTE, None)], 0).await;
    fund(&net, vec![(&recipient_ua, NOTE, None)], 0).await;
    recipient
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    check_client_balances!(recipient, i: (2 * NOTE) o: 0 s: 0 t: 0);

    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_address(PoolType::ORCHARD), PAYMENT, None)],
    )
    .await
    .expect("the funded wallet covers the payment and its fee");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    // The payment plus its fee exceeds either note alone, so the send
    // consumed both and returned the rest as change.
    check_client_balances!(recipient, i: (2 * NOTE - PAYMENT - fee) o: 0 s: 0 t: 0);
}
#[tokio::test]
async fn mine_to_transparent() {
    use crate::lightclient::LightClient;
    use pepper_sync::wallet::TransparentCoin;

    const FIRST_BLOCKS: u64 = 3;
    let mut net = MockNet::launch().await;
    let mut miner = net.client(HOSPITAL_MUSEUM, None).await;
    let miner_taddr = get_base_address(&miner, PoolType::Transparent).await;
    {
        let mut chain = net.chain.write().await;
        for _ in 0..FIRST_BLOCKS {
            chain.mine_block_rewarding(&miner_taddr, REWARD, vec![]);
        }
    }
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    async fn unfiltered(miner: &LightClient) -> u64 {
        miner
            .wallet()
            .read()
            .await
            .get_filtered_balance::<TransparentCoin, _>(|_, _| true, AccountId::ZERO)
            .expect("a few coinbase rewards sum within the money range")
            .into_u64()
    }
    assert_eq!(unfiltered(&miner).await, FIRST_BLOCKS * REWARD_ZATS);

    net.chain
        .write()
        .await
        .mine_block_rewarding(&miner_taddr, REWARD, vec![]);
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    assert_eq!(unfiltered(&miner).await, (FIRST_BLOCKS + 1) * REWARD_ZATS);
}
#[tokio::test]
async fn mine_to_transparent_coinbase_maturity() {
    use crate::lightclient::LightClient;
    use pepper_sync::wallet::TransparentCoin;

    const FIRST_BLOCKS: u64 = 3;
    let mut net = MockNet::launch().await;
    let mut miner = net.client(HOSPITAL_MUSEUM, None).await;
    let miner_taddr = get_base_address(&miner, PoolType::Transparent).await;
    {
        let mut chain = net.chain.write().await;
        for _ in 0..FIRST_BLOCKS {
            chain.mine_block_rewarding(&miner_taddr, REWARD, vec![]);
        }
    }
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    async fn mature(miner: &LightClient) -> u64 {
        miner
            .wallet()
            .read()
            .await
            .confirmed_balance_excluding_dust::<TransparentCoin>(AccountId::ZERO)
            .expect("a few coinbase rewards sum within the money range")
            .into_u64()
    }
    check_client_balances!(miner, i: 0 o: 0 s: 0 t: 0);
    assert_eq!(mature(&miner).await, 0);

    net.chain
        .write()
        .await
        .mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    assert_eq!(mature(&miner).await, FIRST_BLOCKS * REWARD_ZATS);
}
#[tokio::test]
async fn mine_to_orchard() {
    const ORCHARD_BLOCKS: u64 = 3;
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(
        mock_activation_heights_deferring_nu6_3(DEFERRED_NU6_3),
    ))
    .await;
    let mut miner = net.client(HOSPITAL_MUSEUM, None).await;
    let miner_sapling = get_base_address(&miner, PoolType::Shielded(ShieldedPool::Sapling)).await;
    let miner_ua = get_base_address(&miner, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.mine_block_rewarding_shielded(&miner_sapling, REWARD, vec![])
        .await;
    for _ in 0..ORCHARD_BLOCKS {
        net.mine_block_rewarding_shielded(&miner_ua, REWARD, vec![])
            .await;
    }
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    check_client_balances!(miner, i: 0 o: (ORCHARD_BLOCKS * REWARD_ZATS) s: REWARD_ZATS t: 0);

    net.mine_block_rewarding_shielded(&miner_ua, REWARD, vec![])
        .await;
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    check_client_balances!(miner, i: 0 o: ((ORCHARD_BLOCKS + 1) * REWARD_ZATS) s: REWARD_ZATS t: 0);
}
#[tokio::test]
async fn mine_to_ironwood() {
    const IRONWOOD_BLOCKS: u64 = 3;
    let mut net = MockNet::launch().await;
    let mut miner = net.client(HOSPITAL_MUSEUM, None).await;
    let miner_sapling = get_base_address(&miner, PoolType::Shielded(ShieldedPool::Sapling)).await;
    let miner_ua = get_base_address(&miner, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.mine_block_rewarding_shielded(&miner_sapling, REWARD, vec![])
        .await;
    for _ in 0..IRONWOOD_BLOCKS {
        net.mine_block_rewarding_shielded(&miner_ua, REWARD, vec![])
            .await;
    }
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    check_client_balances!(miner, i: (IRONWOOD_BLOCKS * REWARD_ZATS) o: 0 s: REWARD_ZATS t: 0);

    net.mine_block_rewarding_shielded(&miner_ua, REWARD, vec![])
        .await;
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    check_client_balances!(miner, i: ((IRONWOOD_BLOCKS + 1) * REWARD_ZATS) o: 0 s: REWARD_ZATS t: 0);
}
#[tokio::test]
async fn ironwood_miner_coinbase_distribution() {
    use crate::testutils::mock_activation_heights_with;
    use crate::testutils::mock_indexer::MockChain;

    const NU6_3: u32 = 5;
    const ORCHARD_BLOCKS: u64 = (NU6_3 - 2) as u64;
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(
        mock_activation_heights_with(|era| era.set_nu6_3(Some(NU6_3)).set_nu7(None)),
    ))
    .await;
    let mut miner = net.client(HOSPITAL_MUSEUM, None).await;
    let miner_sapling = get_base_address(&miner, PoolType::Shielded(ShieldedPool::Sapling)).await;
    let miner_ua = get_base_address(&miner, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.mine_block_rewarding_shielded(&miner_sapling, REWARD, vec![])
        .await;
    for _ in 0..ORCHARD_BLOCKS {
        net.mine_block_rewarding_shielded(&miner_ua, REWARD, vec![])
            .await;
    }
    assert_eq!(net.chain.read().await.tip(), NU6_3 - 1);
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    // Every coinbase so far predates the activation, so the unified
    // address's rewards are legacy Orchard notes and the Ironwood pool
    // is empty.
    check_client_balances!(miner, i: 0 o: (ORCHARD_BLOCKS * REWARD_ZATS) s: REWARD_ZATS t: 0);

    // The activation block's coinbase lands in Ironwood.
    net.mine_block_rewarding_shielded(&miner_ua, REWARD, vec![])
        .await;
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    check_client_balances!(miner, i: REWARD_ZATS o: (ORCHARD_BLOCKS * REWARD_ZATS) s: REWARD_ZATS t: 0);
}
#[tokio::test]
async fn send_mined_ironwood_to_ironwood() {
    const IRONWOOD_BLOCKS: u64 = 3;
    const AMOUNT_TO_SEND: u64 = 10_000;
    let fee = fee_tables::one_to_one(Some(ShieldedPool::Ironwood), PoolType::IRONWOOD, true);
    let mut net = MockNet::launch().await;
    let mut miner = net.client(HOSPITAL_MUSEUM, None).await;
    let miner_ua = get_base_address(&miner, PoolType::Shielded(ShieldedPool::Orchard)).await;
    for _ in 0..IRONWOOD_BLOCKS {
        net.mine_block_rewarding_shielded(&miner_ua, REWARD, vec![])
            .await;
    }
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    let funded = IRONWOOD_BLOCKS * REWARD_ZATS;
    check_client_balances!(miner, i: funded o: 0 s: 0 t: 0);

    from_inputs::quick_send(
        &mut miner,
        vec![(&miner_ua, AMOUNT_TO_SEND, Some("Scenario test: engage!"))],
    )
    .await
    .expect("the funded wallet covers the payment and its fee");
    // The miner mines the confirming block itself, collecting a fresh
    // reward plus the fee it paid.
    let reward_with_fee = Zatoshis::from_u64(REWARD_ZATS + fee)
        .expect("one reward plus one fee stays within the money range");
    net.mine_mempool_rewarding_shielded(&miner_ua, reward_with_fee)
        .await;
    miner
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    let balance = miner
        .account_balance(AccountId::ZERO)
        .await
        .expect("a seed wallet holds account zero");
    assert_eq!(balance.unconfirmed_ironwood_balance, Some(Zatoshis::ZERO));
    assert_eq!(
        balance
            .confirmed_ironwood_balance
            .expect("a seed wallet tracks the Ironwood pool")
            .into_u64(),
        funded + REWARD_ZATS
    );
}
#[tokio::test]
async fn verify_old_wallet_uses_server_height_in_send() {
    const SYNCED_BLOCKS: u32 = 5;
    const HIDDEN_BLOCKS: u32 = 2;
    const PAYMENT: u64 = 10_000;

    let mut net = MockNet::launch().await;
    let mut sender = net.client(HOSPITAL_MUSEUM, None).await;
    let sender_ua = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&sender_ua, FUNDING, None)], SYNCED_BLOCKS).await;
    sender
        .sync_and_await()
        .await
        .expect("the mock chain serves every block the wallet requests");
    let synced_height = sender
        .wallet()
        .read()
        .await
        .sync_state
        .fully_scanned_height()
        .expect("a wallet that has completed a sync session records a fully scanned height");
    assert_eq!(u32::from(synced_height), net.chain.read().await.tip());

    // Blocks the wallet has not seen: it stays at the height it last
    // synced to, two behind the server's tip.
    net.chain.write().await.mine_empty_blocks(HIDDEN_BLOCKS);
    let stale_height = sender
        .wallet()
        .read()
        .await
        .sync_state
        .fully_scanned_height()
        .expect("a wallet that has completed a sync session records a fully scanned height");
    assert_eq!(stale_height, synced_height);
    assert_eq!(
        u32::from(stale_height) + HIDDEN_BLOCKS,
        net.chain.read().await.tip()
    );

    // A send built from the server's height, not the wallet's, is one
    // the chain accepts into its mempool.
    from_inputs::quick_send(
        &mut sender,
        vec![(
            &external_address(PoolType::ORCHARD),
            PAYMENT,
            Some("Interrupting sync!!"),
        )],
    )
    .await
    .expect("the funded wallet covers the payment and its fee");
    assert_eq!(net.chain.read().await.mempool_len(), 1);
}
